# TUI 执行反馈、消息送达与稳定阅读验收

日期：2026-11-17。版本：`0.151.0-rc1`。结论：本轮 TUI 范围通过本地验收；未提交、安装或发布。这不代表同版本其他并行开发功能完成验收。

## 验收对象

- 真实执行阶段、人工审批/提问优先、静默附加说明、明确终态，以及已定稿阶段说明的时间与模型归属。
- 按 Turn/request 身份跟踪提交、已接受排队、主动暂缓、开始和终态；丢回执对账、重放幂等、同文不同消息、编辑冲突及完整草稿/附件恢复。
- 回看与鼠标按下即停止跟随，按原文来源恢复阅读和选区；新内容入口、窗口重排、折叠/归档、流落历史和原文复制。
- 既有输入草稿拖选复制的回归。

实施契约见[交互决策](../../decisions/2026-11-17-tui-progress-delivery-viewport.md)。沿用现有持久队列与调度协议；新增前台 `model_request_started` 事件只证明请求进入 Provider 调用，不表示模型已读或执行成功。

## 原生测试和工程检查

| 检查 | 结果 | 覆盖范围 |
| --- | --- | --- |
| `cargo test -p willdeep --bin willdeep tui:: --no-fail-fast` | 622 通过、0 失败、2 忽略 | 全部默认 TUI 单元测试，包括本轮状态、身份、恢复、来源和旧交互回归 |
| `cargo test -p willdeep --bin willdeep agent_event_json_has_stable_runtime_schema -- --nocapture` | 1 通过 | 新前台请求事件的精确 JSON schema |
| `cargo test -p willdeep --bin willdeep editor:: --no-fail-fast` | 11 通过 | 输入编辑器和原文选择 |
| `cargo test -p willdeep-core request_progress_tests -- --nocapture` | 2 通过 | 准备→前台请求→Provider 顺序，实际自动压缩结束后才进入前台等待 |
| `cargo test -p willdeep --bin willdeep tui::turn_status::tests -- --nocapture` | 19 通过 | 最后一次人工等待优先条件修正后复核；属于上述 TUI 范围，不重复累计 |
| `cargo clippy -p willdeep --bin willdeep -- -D warnings` | 通过 | CLI 生产目标及依赖 |
| `cargo build -p willdeep --bin willdeep` | 通过 | 本机 debug CLI |
| 定向 `rustfmt --check --config skip_children=true`、`git diff --check` | 通过 | 本轮 Rust 文件和工作区差异 |
| 版本一致性与源码长度检查 | 通过 | `0.151.0-rc1` 一致，单文件上限 3000 行无违规 |
| Ruby 夹具语法、Node 终端解码器语法 | 通过 | 两套 PTY 驱动和帧解码 |

去重原生测试共 636 项。默认忽略的两项是手工流式渲染性能与定速端到显示延迟测试，本轮没有执行。没有运行整个工作区的全部测试或跨平台 CI。

## 真实终端

使用实际 `target/debug/willdeep`、独立 PTY、私有 HOME/WILLDEEP_HOME/XDG/工作区、随机回环端口的合成 SSE Provider。每种布局实际提交两条消息；没有真实模型调用或真实凭据。只停止夹具自己的 daemon。

两套验收记录的二进制 SHA-256 相同，且各自在结束前确认产物没有变化：

`81537596bde5bcc649664c5b1c4d6ba1db01008df8aa81551f2ef52d08301577`

完整交互脚本在 full、compact 各通过 9 项，共 18 项：

1. Provider 尚未输出时显示真实的 Waiting for model。
2. 回看期间收到流式正文后显示新内容提示，原历史标记保持屏幕行位置。
3. 覆盖鼠标 Down→增量→首次 Drag 的空窗，并确认后续增量不改变整段原文的高亮。
4. 宽度 100→84→100，各次等待越过 resize 边界的新应用帧；整段中文原文的高亮和 OSC 52 复制保持精确一致。
5. 忙时补充消息获得已接受排队反馈，第一请求结束前不启动第二个请求。
6. 队列公开优先处理可能停止当前轮次的实际语义。
7. 已接受消息随后恰好启动一次，另行编辑的中文/emoji 草稿保留。
8. 点击返回最新显示后台回复，且不提交草稿。
9. Runtime 持久保存合成回复。

输入草稿脚本在 full、compact 及各自 SSH 路由共通过 14 项，覆盖中文、emoji、跨行高亮、Esc 保留草稿、y/q 正常输入和精确原文 OSC 52 payload。普通路由不发送复制快捷键；SSH 路由只由私有 PTY 捕获终端序列。

原始 JSON、Markdown、ANSI 和逐单元格屏幕证据：

- [完整交互 JSON](../../../build/verification/0.151.0-rc1/tui-interactions-verified/integration.json) / [Markdown](../../../build/verification/0.151.0-rc1/tui-interactions-verified/integration.md)。包含两次聊天复制的预期/实际原文、63 字节 payload 和四次 resize 帧边界。
- [输入草稿 JSON](../../../build/verification/0.151.0-rc1/tui-prompt-selection-verified/integration.json) / [Markdown](../../../build/verification/0.151.0-rc1/tui-prompt-selection-verified/integration.md)。

`build/` 证据为本机产物；本文件及[汇总 JSON](tui-interactions.json)记录可随代码审阅的结果。

## 失败尝试与修正

此前 CLI 测试曾被共享改动中的错误类型 `Debug` 约束和缺失的 `probe_config` 阻断，不计作测试通过。插件测试改用错误现有的 Display 输出；探测函数由并行改动补齐。

首次完整 TUI 执行得到 618 通过、4 失败。修正了流切换时旧 Reply 收尾覆盖新阶段、待发通知遮住明确失败结果；另外对齐后台阶段提示的旧断言，并按字符显示宽度读取 TestBackend，避免把中文占位单元格误当正文空格。随后完整回归得到 622 通过。

此前严格 Clippy 的三个条件简化诊断已修正，最终命令通过。较早 PTY 尝试在复制后保留 Chat 焦点，夹具却直接按 Enter，导致草稿未提交；修正为点击可见输入框后提交，保留严格入队断言。失败尝试保存在 `build/verification/0.151.0-rc1/tui-interactions/` 与 `tui-interactions-final/`，不作为成功证据。

早期一次验收沿用旧聊天复制路径，触发过宿主剪贴板写入尝试，是否写入成功未核实。此后聊天、输入草稿和代码块统一使用同一路由；本报告计入的最终 PTY 验收只捕获私有 OSC 52，不转发到宿主终端或系统剪贴板。OSC 52 没有终端写入回执，因此验证的是精确发送内容，不宣称外部终端已实际写入剪贴板。

## 边界

真实 PTY 运行于本机 macOS、英文界面；中英日状态优先级和两布局绘制由 Rust 测试覆盖。未做真实服务断网或真实模型故障注入，丢回执、重连旧事件、过期编辑等竞态通过隔离原生测试验证。没有实现 `/btw`、LSP 安装提示或其余三项后续建议。其他会话的未提交修改保留。
